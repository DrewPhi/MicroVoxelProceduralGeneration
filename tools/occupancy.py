"""Occupancy-first hard-min fitting; native certification before declaring exact.
Least-squares updates project violated sign margins. Full voxel scans rank
restart candidates; counterexamples feed subsequent restarts. No claim of global
convergence or guaranteed representability with fixed n.
"""
import numpy as np
from fit import features, predict, solve


def native_check(block, coeff, examples=512):
    edge=block.shape[0];errors=0;bad=[]
    for start in range(0,block.size,65536):
        ids=np.arange(start,min(start+65536,block.size));yy,zz,xx=np.unravel_index(ids,block.shape)
        p=(np.column_stack((xx,yy,zz))+.5)/edge*2-1
        labels=block[yy,zz,xx]>0;pred=predict(features(p),coeff)>0
        wrong=np.flatnonzero(labels!=pred);errors+=len(wrong)
        if len(wrong):
            take=wrong[np.linspace(0,len(wrong)-1,min(16,len(wrong)),dtype=int)]
            bad.extend(ids[take].tolist())
    return int(errors),np.array(bad[:examples],dtype=int)


def fit_occupancy(p,y,n,restarts,iterations,rng,initial,progress,block,
                  patience=24,min_restarts=16,warm=None):
    edge=block.shape[0];scale=max(float(np.std(y)),.1);margin=.02
    # Exact constant regions require no fitting effort.
    if np.all(block>0) or np.all(block<=0):
        c=np.zeros((n,10));c[:,0]=1. if np.all(block>0) else -1.
        fit_occupancy.last_stats=dict(native_errors=0,stop_reason='exact_native_match',restarts_used=0)
        return c,float(np.mean((predict(features(p),c)-y)**2))
    best=initial.copy() if initial is not None else np.zeros((n,10))
    best_errors,bad=native_check(block,best)
    if warm is not None:
        count,extra=native_check(block,warm)
        if count<best_errors:best,best_errors,bad=warm.copy(),count,extra
    training_ids=set();points=p.copy();values=y.copy()
    def add_examples(ids):
        nonlocal points,values
        ids=np.array([int(i) for i in ids if int(i) not in training_ids],dtype=int)
        if not len(ids) or len(points)>=32768:return
        ids=ids[:32768-len(points)];training_ids.update(ids.tolist())
        yy,zz,xx=np.unravel_index(ids,block.shape)
        points=np.vstack((points,(np.column_stack((xx,yy,zz))+.5)/edge*2-1))
        values=np.r_[values,block[yy,zz,xx]]
    add_examples(bad)
    def score(c,a,sign):
        prediction=predict(a,c)
        return int(np.count_nonzero((prediction>0)!=(sign>0))),float(np.mean(np.maximum(0,margin-sign*prediction)**2))
    stale=0;reason='restart_budget';used=0
    for restart in range(restarts):
        if best_errors==0:reason='exact_native_match';break
        a=features(points);sign=np.where(values>0,1.,-1.)
        if restart==0 or restart%4==0:coeff=best/scale
        else:
            centers=points[rng.choice(len(points),n,replace=False)]
            owners=np.argmin(((points[:,None,:]-centers[None,:,:])**2).sum(axis=2),axis=1)
            coeff=np.zeros((n,10))
            for k in range(n):
                mask=owners==k;coeff[k]=solve(a[mask],values[mask]/scale,coeff[k],1e-4)
        current=score(coeff,a,sign)
        stagnant_iterations=0;fewest_training_errors=current[0]
        for iteration in range(iterations):
            q=a@coeff.T;owners=np.argmin(q,axis=1);candidate=coeff.copy()
            for k in range(n):
                mask=(sign>0)|(owners==k)
                if mask.sum()<10:continue
                target=q[mask,k].copy();positive=sign[mask]>0
                target[positive]=np.maximum(target[positive],margin)
                target[~positive]=np.minimum(target[~positive],-margin)
                candidate[k]=solve(a[mask],target,coeff[k],1e-4)
            accepted=False
            for alpha in (1.,.5,.25,.125,.0625,.015625):
                trial=coeff+alpha*(candidate-coeff);trial_score=score(trial,a,sign)
                if trial_score[0]<current[0] or (trial_score[0]==current[0] and trial_score[1]<current[1]-1e-12):
                    coeff=trial;current=trial_score;accepted=True;break
            if current[0]<fewest_training_errors:
                fewest_training_errors=current[0];stagnant_iterations=0
            else:stagnant_iterations+=1
            if not accepted or current==(0,0.) or stagnant_iterations>=64:break
        physical=coeff*scale;errors,bad=native_check(block,physical)
        previous=best_errors
        if errors<best_errors or (errors==best_errors and score(coeff,a,sign)<score(best/scale,a,sign)):
            best=physical.copy();best_errors=errors
        stale=0 if best_errors<previous else stale+1
        add_examples(bad);used=restart+1
        loss=float(np.mean((predict(features(p),best)-y)**2))
        progress(used,loss,native_errors=best_errors,native_cells=block.size,
                 training_points=len(points),plateau_restarts=stale)
        if best_errors==0:reason='exact_native_match';break
        if used>=min_restarts and stale>=patience:reason='plateau_not_exact';break
    fit_occupancy.last_stats=dict(native_errors=best_errors,stop_reason=reason,restarts_used=used,training_points=len(points))
    return best,float(np.mean((predict(features(p),best)-y)**2))
